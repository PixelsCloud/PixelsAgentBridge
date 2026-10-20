// Opt-in, bounded own-window input diagnostic; not part of the product.
// Only tagged mouse events over this process are observed; no keyboard hooks.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <windowsx.h>
#include <tlhelp32.h>
#include <chrono>
#include <iostream>
#include <stdexcept>
#include <string>
#include <thread>
using Clock=std::chrono::steady_clock;
struct Display { struct {RECT DesktopCoordinates;} screen; };
static bool consume=false;
static bool no_uia=false, explicit_activate=false, responsive_pump=false;
static void fixture_sleep(DWORD ms){if(responsive_pump)MsgWaitForMultipleObjects(0,nullptr,FALSE,ms,QS_ALLINPUT);else Sleep(ms);}
static bool blocking_loop=false;
static bool focus_fixture=false;
static BOOL fixture_show(HWND window,int command) {
 auto result=ShowWindow(window,command);
 if(focus_fixture){ShowWindow(window,SW_SHOWNORMAL);SetForegroundWindow(window);}
 return result;
}
static BOOL fixture_peek(MSG* m,HWND w,UINT low,UINT high,UINT flags) {
 if(!blocking_loop)return PeekMessageW(m,w,low,high,flags);
 static UINT_PTR timer=SetTimer(nullptr,0,100,nullptr);
 auto result=GetMessageW(m,w,low,high);
 return result>0 && m->message!=WM_TIMER;
}
static bool dispatching=false;
static LRESULT fixture_dispatch(const MSG* m) {
 if(m->message==WM_LBUTTONDOWN || m->message==WM_LBUTTONUP)std::cerr << "{\"dispatch\":"<<m->message<<",\"time\":"<<m->time<<"}\n"<<std::flush;
 dispatching=true;auto r=DispatchMessageW(m);dispatching=false;return r;
}
static LRESULT fixture_default(HWND w,UINT m,WPARAM p,LPARAM l) {
 if(no_uia && m==WM_GETOBJECT)return 0;
 if(explicit_activate && m==WM_MOUSEACTIVATE)return MA_ACTIVATE;
 INPUT_MESSAGE_SOURCE source{};const bool source_ok=GetCurrentInputMessageSource(&source);
 if(m==WM_LBUTTONDOWN || m==WM_LBUTTONUP || m==WM_MOUSEACTIVATE || m==WM_ACTIVATE || m==WM_SETFOCUS)
  std::cerr << "{\"message\":" << m << ",\"tick\":" << GetTickCount64() << ",\"extra\":" << GetMessageExtraInfo() << ",\"source_ok\":" << source_ok << ",\"device\":" << source.deviceType << ",\"origin\":" << source.originId << ",\"dispatching\":" << dispatching << ",\"sent\":" << InSendMessageEx(nullptr) << ",\"foreground\":" << (GetForegroundWindow()==w ? "true":"false") << "}\n" << std::flush;
 if(consume && (m==WM_LBUTTONDOWN || m==WM_LBUTTONUP || m==WM_MOUSEWHEEL))return 0;
 return DefWindowProcW(w,m,p,l);
}
#define DefWindowProcW fixture_default
#define DispatchMessageW fixture_dispatch
#define PeekMessageW fixture_peek
#define ShowWindow fixture_show
#define Sleep fixture_sleep
#include "../../crates/desktop-media/native/windows/input_fixture.h"
#undef DefWindowProcW
#undef DispatchMessageW
#undef PeekMessageW
#undef ShowWindow
#undef Sleep
static LRESULT CALLBACK observe_mouse(int code,WPARAM message,LPARAM payload) {
 if(code==HC_ACTION) {
  const auto* e=reinterpret_cast<const MSLLHOOKSTRUCT*>(payload);
  if(e->dwExtraInfo==100 && (message==WM_LBUTTONDOWN || message==WM_LBUTTONUP)){
   DWORD pid=0;GetWindowThreadProcessId(WindowFromPoint(e->pt),&pid);
   if(pid==GetCurrentProcessId())std::cerr<<"{\"low_level\":"<<message<<",\"flags\":"<<e->flags<<",\"time\":"<<e->time<<"}\n"<<std::flush;
  }
 }
 return CallNextHookEx(nullptr,code,message,payload);
}
int main(int argc,char** argv){
 if(argc!=2 || std::string(argv[1]).find("-self")==std::string::npos){std::cerr<<"Specify a bounded -self diagnostic mode\n";return 2;}
 SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
 std::string mode=argc==2 ? argv[1]:"default";
 consume=mode.find("consume")==0;
 responsive_pump=mode.find("responsive")!=std::string::npos;
 no_uia=mode.find("nouia")!=std::string::npos;explicit_activate=mode.find("activate")!=std::string::npos;
 blocking_loop=mode.find("blocking")!=std::string::npos;
 focus_fixture=mode.find("focus")!=std::string::npos;
 const bool self=mode.find("-self")!=std::string::npos;
 HHOOK observation=SetWindowsHookExW(WH_MOUSE_LL,observe_mouse,GetModuleHandleW(nullptr),0);
 if(!observation)return 2;
 std::thread injection;
 if(self)injection=std::thread([mode] {
   HWND window=nullptr;
   for(unsigned i=0;i<100;++i){window=FindWindowW(L"PixelsRemoteDesktopInputAcceptance",nullptr);DWORD pid=0;if(window)GetWindowThreadProcessId(window,&pid);if(pid==GetCurrentProcessId())break;window=nullptr;Sleep(50);}
   if(!window)return;
   Sleep(600);
   RECT area{};GetClientRect(window,&area);POINT point{area.right/2,area.bottom/2};ClientToScreen(window,&point);
   if(WindowFromPoint(point)!=window){auto exposed=WindowFromPoint(point);wchar_t cls[128]{};GetClassNameW(exposed,cls,128);DWORD pid=0;GetWindowThreadProcessId(exposed,&pid);std::cerr<<"fixture is not exposed: visible="<<IsWindowVisible(window)<<" iconic="<<IsIconic(window)<<" point="<<point.x<<","<<point.y<<" bounds="<<area.right<<","<<area.bottom<<" otherpid="<<pid<<" ownpid="<<GetCurrentProcessId()<<"\n";std::wcerr<<L"exposed class="<<cls<<L"\n";return;}
   for(int key : {VK_LBUTTON,VK_CONTROL,VK_SHIFT,VK_MENU})if(GetAsyncKeyState(key)&0x8000){std::cerr<<"local input is held\n";return;}
   POINT previous{};GetCursorPos(&previous);
   int vx=GetSystemMetrics(SM_XVIRTUALSCREEN),vy=GetSystemMetrics(SM_YVIRTUALSCREEN),vw=GetSystemMetrics(SM_CXVIRTUALSCREEN),vh=GetSystemMetrics(SM_CYVIRTUALSCREEN);
   INPUT inputs[2]{};inputs[0].type=INPUT_MOUSE;inputs[0].mi.dx=LONG(((int64_t(point.x-vx)*65536)+32768)/vw);inputs[0].mi.dy=LONG(((int64_t(point.y-vy)*65536)+32768)/vh);inputs[0].mi.dwFlags=MOUSEEVENTF_MOVE|MOUSEEVENTF_ABSOLUTE|MOUSEEVENTF_VIRTUALDESK;
   inputs[1].type=INPUT_MOUSE;inputs[1].mi.dwFlags=MOUSEEVENTF_LEFTDOWN;
   const bool tagged=mode.find("tagged")!=std::string::npos;
   if(tagged){inputs[0].mi.dwExtraInfo=100;inputs[1].mi.dwExtraInfo=100;}
   UINT sent=0;
   if(mode.find("separate")!=std::string::npos){sent=SendInput(1,inputs,sizeof(INPUT));Sleep(250);sent+=SendInput(1,inputs+1,sizeof(INPUT));}
   else if(mode.find("combined")!=std::string::npos){inputs[0].mi.dwFlags|=MOUSEEVENTF_LEFTDOWN;sent=SendInput(1,inputs,sizeof(INPUT));}
   else sent=SendInput(2,inputs,sizeof(INPUT));
   Sleep(400);INPUT up{};up.type=INPUT_MOUSE;up.mi.dwFlags=MOUSEEVENTF_LEFTUP;if(tagged)up.mi.dwExtraInfo=100;const auto released=SendInput(1,&up,sizeof(INPUT));
   std::cerr<<"{\"injected\":"<<sent<<",\"released\":"<<released<<"}\n"<<std::flush;
   POINT now{};GetCursorPos(&now);if(now.x==point.x && now.y==point.y && GetForegroundWindow()==window)SetCursorPos(previous.x,previous.y);
 });
 Display d;d.screen.DesktopCoordinates={0,0,GetSystemMetrics(SM_CXSCREEN),GetSystemMetrics(SM_CYSCREEN)};
 int result=0;
 try {input_fixture(d,self ? 5:90);}catch(const std::exception& e){std::cerr<<e.what();result=1;}
 if(injection.joinable())injection.join();
 UnhookWindowsHookEx(observation);
 HANDLE snapshot=CreateToolhelp32Snapshot(TH32CS_SNAPMODULE,GetCurrentProcessId());
 MODULEENTRY32W module{};module.dwSize=sizeof(module);
 if(snapshot!=INVALID_HANDLE_VALUE){if(Module32FirstW(snapshot,&module))do{std::wcerr<<L"MODULE "<<module.szExePath<<L"\n";}while(Module32NextW(snapshot,&module));CloseHandle(snapshot);}
 return result;
}
