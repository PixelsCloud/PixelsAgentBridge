#pragma once
#include <functional>

// An owned layered popup composes over video without reading GPU pixels.
// Layered child windows require a compatibility manifest that media workers
// need not have. Ownership keeps this popup in the viewer's native window group.
// Its mouse messages never enter ViewerInput or leave this machine.
class ViewerStats {
    HWND window_ = nullptr, parent_ = nullptr;
    HFONT font_ = nullptr;
    UINT dpi_ = 0;
    std::wstring text_;
    std::function<void()> clicked_;
    static LRESULT CALLBACK procedure(HWND window, UINT message, WPARAM key, LPARAM param) {
        auto* self = reinterpret_cast<ViewerStats*>(GetWindowLongPtrW(window,GWLP_USERDATA));
        if (message == WM_NCCREATE) {
            self = static_cast<ViewerStats*>(reinterpret_cast<CREATESTRUCTW*>(param)->lpCreateParams);
            SetWindowLongPtrW(window,GWLP_USERDATA,reinterpret_cast<LONG_PTR>(self));
        }
        if (!self) return DefWindowProcW(window,message,key,param);
        if (message == WM_MOUSEACTIVATE) return MA_NOACTIVATE;
        if (message == WM_LBUTTONUP) {
            self->clicked_();
            InvalidateRect(window,nullptr,TRUE); return 0;
        }
        if (message == WM_ERASEBKGND) return 1;
        if (message == WM_PAINT) {
            PAINTSTRUCT paint{}; HDC dc = BeginPaint(window,&paint); RECT rect{}; GetClientRect(window,&rect);
            HBRUSH background = CreateSolidBrush(RGB(20,29,39)); FillRect(dc,&rect,background); DeleteObject(background);
            SetBkMode(dc,TRANSPARENT); SetTextColor(dc,RGB(234,243,246)); auto old = SelectObject(dc,self->font_);
            const int padding = MulDiv(12,self->dpi_,96);
            InflateRect(&rect,-padding,-padding);
            DrawTextW(dc,self->text_.c_str(),-1,&rect,DT_LEFT|DT_TOP|DT_NOPREFIX|DT_WORDBREAK);
            SelectObject(dc,old); EndPaint(window,&paint); return 0;
        }
        return DefWindowProcW(window,message,key,param);
    }
public:
    ~ViewerStats() { if (window_ && IsWindow(window_)) DestroyWindow(window_); if (font_) DeleteObject(font_); }
    void update(HWND parent, const uint8_t* bytes, size_t size, std::function<void()> clicked) {
        if (size > 2048) throw std::runtime_error("Viewer statistics too large");
        const int count = MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,reinterpret_cast<const char*>(bytes),static_cast<int>(size),nullptr,0);
        if (size && !count) throw std::runtime_error("Invalid viewer statistics text");
        text_.resize(count);
        if (count) MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,reinterpret_cast<const char*>(bytes),static_cast<int>(size),text_.data(),count);
        parent_ = parent; clicked_ = std::move(clicked);
        if (!window_ && size) {
            WNDCLASSW cls{}; cls.lpfnWndProc = procedure; cls.hInstance = GetModuleHandleW(nullptr);
            cls.lpszClassName = L"PixelsRemoteDesktopStats"; cls.hCursor = LoadCursorW(nullptr,IDC_ARROW);
            RegisterClassW(&cls);
            window_ = CreateWindowExW(WS_EX_LAYERED|WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW,cls.lpszClassName,L"",WS_POPUP,
                0,0,1,1,GetAncestor(parent,GA_ROOT),nullptr,cls.hInstance,this);
            if (!window_ || !SetLayeredWindowAttributes(window_,0,215,LWA_ALPHA)) throw std::runtime_error("Cannot create viewer statistics: "+std::to_string(GetLastError()));
        }
        if (window_) { layout(); InvalidateRect(window_,nullptr,TRUE); }
    }
    void layout() {
        if (!window_ || !IsWindow(parent_)) return;
        const bool visible = !text_.empty() && IsWindowVisible(parent_) && !IsIconic(GetAncestor(parent_,GA_ROOT));
        if (bool(IsWindowVisible(window_)) != visible) ShowWindow(window_,visible ? SW_SHOWNOACTIVATE : SW_HIDE);
        const UINT dpi = GetDpiForWindow(parent_);
        if (dpi != dpi_) {
            dpi_ = dpi; if (font_) DeleteObject(font_);
            font_ = CreateFontW(-MulDiv(12,dpi,96),0,0,0,FW_NORMAL,FALSE,FALSE,FALSE,DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,CLIP_DEFAULT_PRECIS,CLEARTYPE_QUALITY,DEFAULT_PITCH,L"Segoe UI");
        }
        RECT bounds{}; GetClientRect(parent_,&bounds);
        const int gap = MulDiv(12,dpi_,96), lines = 1+static_cast<int>(std::count(text_.begin(),text_.end(),L'\n'));
        const int width = (std::min)(MulDiv(360,dpi_,96),static_cast<int>((std::max)(1L,bounds.right-gap*2)));
        const int height = (std::min)(MulDiv(24+lines*19,dpi_,96),static_cast<int>((std::max)(1L,bounds.bottom-gap*2)));
        const int x = (std::max)(0L,bounds.right-width-gap), y = (std::max)(0L,bounds.bottom-height-gap);
        RECT current{}; GetWindowRect(window_,&current); POINT point{x,y}; ClientToScreen(parent_,&point);
        if (current.left!=point.x || current.top!=point.y || current.right-current.left!=width || current.bottom-current.top!=height) {
            SetWindowPos(window_,nullptr,point.x,point.y,width,height,SWP_NOACTIVATE|SWP_NOZORDER);
            SetWindowRgn(window_,CreateRoundRectRgn(0,0,width+1,height+1,MulDiv(12,dpi_,96),MulDiv(12,dpi_,96)),TRUE);
        }
    }
};
