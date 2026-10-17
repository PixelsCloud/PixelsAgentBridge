// Bounded own-window fixture for real capture/transport/render/input acceptance.
// Changes cursor only through WM_SETCURSOR in this window. No hooks or input injection.
#define WIN32_LEAN_AND_MEAN
#ifndef UNICODE
#define UNICODE
#endif
#ifndef _UNICODE
#define _UNICODE
#endif
#include <windows.h>
#include <windowsx.h>
#include <algorithm>
#include <chrono>
#include <cstdint>
#include <iostream>
#include <string>

namespace {
HCURSOR color_cursor = nullptr, mask_cursor = nullptr;
unsigned clicks = 0, key_down = 0, key_up = 0;
POINT last_click{-1,-1};
unsigned tick = 0;
int zone_at(HWND window, POINT p) {
    RECT client{}; GetClientRect(window,&client);
    if (!PtInRect(&client,p)) return -1;
    return (std::min)(3, int(p.x*4/(std::max)(1L,client.right)));
}
LRESULT CALLBACK procedure(HWND window,UINT message,WPARAM w,LPARAM l) {
    if(message==WM_SETCURSOR && LOWORD(l)==HTCLIENT) {
        POINT p{};GetCursorPos(&p);ScreenToClient(window,&p);
        switch(zone_at(window,p)) {
            case 0:SetCursor(LoadCursorW(nullptr,IDC_ARROW));break;
            case 1:SetCursor(color_cursor);break;
            case 2:SetCursor(mask_cursor);break;
            case 3:SetCursor(nullptr);break;
            default:return DefWindowProcW(window,message,w,l);
        }
        return TRUE;
    }
    if(message==WM_LBUTTONDOWN) {
        ++clicks;last_click={GET_X_LPARAM(l),GET_Y_LPARAM(l)};
        ClientToScreen(window,&last_click);
    }
    if(message==WM_KEYDOWN && w==VK_F24)++key_down;
    if(message==WM_KEYUP && w==VK_F24)++key_up;
    if(message==WM_PAINT) {
        PAINTSTRUCT paint{};HDC dc=BeginPaint(window,&paint);RECT client{};GetClientRect(window,&client);
        const wchar_t* labels[]{L"1: system arrow",L"2: color, hotspot (5,7)",L"3: AND/XOR, hotspot (8,8)",L"4: hidden cursor"};
        for(int zone=0;zone<4;++zone) {
            RECT r{client.right*zone/4,0,client.right*(zone+1)/4,client.bottom};
            HBRUSH brush=CreateSolidBrush(RGB(70+zone*25,90+zone*15,120+zone*10));
            FillRect(dc,&r,brush);DeleteObject(brush);
            SetBkMode(dc,TRANSPARENT);SetTextColor(dc,RGB(255,255,255));
            TextOutW(dc,r.left+12,50,labels[zone],lstrlenW(labels[zone]));
        }
        RECT stripe{0,0,client.right,24};
        HBRUSH brush=CreateSolidBrush(RGB((tick*7)%200+30,(tick*11)%200+30,(tick*13)%200+30));
        FillRect(dc,&stripe,brush);DeleteObject(brush);
        EndPaint(window,&paint);return 0;
    }
    if(message==WM_TIMER) {
        ++tick;RECT stripe{};GetClientRect(window,&stripe);stripe.bottom=24;InvalidateRect(window,&stripe,FALSE);return 0;
    }
    return DefWindowProcW(window,message,w,l);
}
HCURSOR make_color() {
    BITMAPINFO bitmap{};bitmap.bmiHeader.biSize=sizeof(BITMAPINFOHEADER);
    bitmap.bmiHeader.biWidth=32;bitmap.bmiHeader.biHeight=-32;
    bitmap.bmiHeader.biPlanes=1;bitmap.bmiHeader.biBitCount=32;bitmap.bmiHeader.biCompression=BI_RGB;
    void* data=nullptr;HBITMAP pixels=CreateDIBSection(nullptr,&bitmap,DIB_RGB_COLORS,&data,nullptr,0);
    if(!pixels)return nullptr;
    auto* bgra=static_cast<uint8_t*>(data);
    for(int y=0;y<32;++y)for(int x=0;x<32;++x) {
        const auto offset=(y*32+x)*4;
        const bool visible=x==5 || y==7 || (x>=12 && x<28 && y>=12 && y<28);
        bgra[offset]=visible?255:0;bgra[offset+1]=visible && x!=5?255:0;
        bgra[offset+2]=visible && y==7?255:0;bgra[offset+3]=visible?255:0;
    }
    const uint8_t mask[128]{};HBITMAP mono=CreateBitmap(32,32,1,1,mask);
    ICONINFO info{};info.xHotspot=5;info.yHotspot=7;info.hbmColor=pixels;info.hbmMask=mono;
    HCURSOR cursor=mono?static_cast<HCURSOR>(CreateIconIndirect(&info)):nullptr;
    DeleteObject(pixels);if(mono)DeleteObject(mono);return cursor;
}
HCURSOR make_mask() {
    uint8_t and_mask[128]{},xor_mask[128]{};
    for(int y=0;y<32;++y)for(int x=0;x<32;++x) {
        const int index=y*4+x/8;const uint8_t bit=uint8_t(0x80>>(x%8));
        // Upper: black/white. Lower: transparent/inverted background.
        if(y>=16)and_mask[index]|=bit;
        if(x>=16)xor_mask[index]|=bit;
    }
    return CreateCursor(GetModuleHandleW(nullptr),8,8,32,32,and_mask,xor_mask);
}
}
int main(int argc,char** argv) {
    const unsigned seconds=argc==2?std::stoul(argv[1]):180;
    if(seconds<10 || seconds>600)return 2;
    if(!SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2))return 3;
    color_cursor=make_color();mask_cursor=make_mask();if(!color_cursor || !mask_cursor)return 4;
    WNDCLASSW cls{};cls.hInstance=GetModuleHandleW(nullptr);cls.lpfnWndProc=procedure;
    cls.hCursor=LoadCursorW(nullptr,IDC_ARROW);cls.lpszClassName=L"PixelsRemoteCursorAcceptance";
    if(!RegisterClassW(&cls))return 5;
    const int width=(std::min)(1320,GetSystemMetrics(SM_CXSCREEN)-80);
    const int height=(std::min)(720,GetSystemMetrics(SM_CYSCREEN)-100);
    HWND window=CreateWindowExW(WS_EX_TOPMOST,cls.lpszClassName,L"Pixels remote cursor acceptance",WS_OVERLAPPEDWINDOW,
        (GetSystemMetrics(SM_CXSCREEN)-width)/2,(GetSystemMetrics(SM_CYSCREEN)-height)/2,width,height,
        nullptr,nullptr,cls.hInstance,nullptr);
    if(!window)return 6;
    ShowWindow(window,SW_SHOWNOACTIVATE);SetTimer(window,1,66,nullptr);
    const auto end=std::chrono::steady_clock::now()+std::chrono::seconds(seconds);
    while(IsWindow(window) && std::chrono::steady_clock::now()<end) {
        MSG m{};while(PeekMessageW(&m,nullptr,0,0,PM_REMOVE)){TranslateMessage(&m);DispatchMessageW(&m);}
        POINT p{};GetCursorPos(&p);const bool owned=WindowFromPoint(p)==window;
        POINT local=p;ScreenToClient(window,&local);const int zone=owned?zone_at(window,local):-1;
        RECT client{};GetClientRect(window,&client);POINT origin{};ClientToScreen(window,&origin);
        CURSORINFO cursor{};cursor.cbSize=sizeof(cursor);const bool known=GetCursorInfo(&cursor)!=0;
        std::cout<<"{\"zone\":"<<zone<<",\"x\":"<<(zone>=0?p.x:-1)<<",\"y\":"<<(zone>=0?p.y:-1)
            <<",\"left\":"<<origin.x<<",\"top\":"<<origin.y<<",\"width\":"<<client.right<<",\"height\":"<<client.bottom
            <<",\"visible\":"<<(known && (cursor.flags&CURSOR_SHOWING)?"true":"false")
            <<",\"clicks\":"<<clicks<<",\"last_click_x\":"<<last_click.x<<",\"last_click_y\":"<<last_click.y
            <<",\"f24_down\":"<<key_down<<",\"f24_up\":"<<key_up<<",\"tick\":"<<tick<<"}\n"<<std::flush;
        Sleep(100);
    }
    if(IsWindow(window)) {
        POINT p{};if(GetCursorPos(&p) && WindowFromPoint(p)==window)SetCursor(LoadCursorW(nullptr,IDC_ARROW));
        DestroyWindow(window);
    }
    DestroyCursor(color_cursor);DestroyCursor(mask_cursor);
    return 0;
}
