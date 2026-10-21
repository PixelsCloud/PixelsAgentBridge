#pragma once
// Generated local windows only. No desktop capture, remote session or global input.
static void viewer_chrome_check() {
    const auto require = [](bool value, const char* message) {
        if (!value) throw std::runtime_error(message);
    };
    ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context;
    check(D3D11CreateDevice(nullptr,D3D_DRIVER_TYPE_HARDWARE,nullptr,D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        nullptr,0,D3D11_SDK_VERSION,&device,nullptr,&context), "Chrome test device");
    HWND host = CreateWindowExW(0,L"STATIC",L"Pixels generated chrome check",WS_OVERLAPPEDWINDOW,
        -20000,-20000,800,600,nullptr,nullptr,GetModuleHandleW(nullptr),nullptr);
    require(host!=nullptr,"Cannot create generated chrome host");
    HANDLE reader=nullptr,writer=nullptr;
    require(CreatePipe(&reader,&writer,nullptr,0)!=FALSE,"Cannot create generated input pipe");
    const HANDLE old_error=GetStdHandle(STD_ERROR_HANDLE);
    SetStdHandle(STD_ERROR_HANDLE,writer);
    try {
        {
            NativePresenter view;
            RECT monitor{0,0,1920,1080};
            view.initialize(device.Get(),640,480,15,monitor,false,false,host);
            view.input_available(true); view.input_enabled(true);
            const HWND surface=FindWindowExW(host,nullptr,L"PixelsRemoteDesktopSurface",nullptr);
            require(surface!=nullptr,"Cannot find generated native surface");
            const auto top = [&] { RECT rect{}; GetWindowRect(surface,&rect);POINT p{rect.left,rect.top};ScreenToClient(host,&p);return p.y; };
            view.pump();
            require(top()==MulDiv(42,GetDpiForWindow(host),96),"Windowed surface lost toolbar inset");
            view.cursor_update({3,1});
            require(top()==0,"Fullscreen surface kept toolbar gap");
            RECT parent{},child{};GetClientRect(host,&parent);GetClientRect(surface,&child);
            require(child.right==parent.right && child.bottom==parent.bottom,"Fullscreen surface did not fill host");
            SendMessageW(surface,WM_KEYDOWN,VK_ESCAPE,0);
            SendMessageW(surface,WM_KEYDOWN,VK_ESCAPE,1LL<<30); // Autorepeat stays local.
            view.cursor_update({3,0});
            SendMessageW(surface,WM_KEYUP,VK_ESCAPE,1LL<<31);
            require(top()==MulDiv(42,GetDpiForWindow(host),96),"Esc exit did not restore toolbar inset");
            view.input_enabled(false);
            view.cursor_update({3,1});
            SendMessageW(surface,WM_KEYDOWN,VK_ESCAPE,0);
            SendMessageW(surface,WM_KEYUP,VK_ESCAPE,1LL<<31);
            view.cursor_update({3,0});
            // Debug visibility is controlled by text from the titlebar, never a bottom-right toggle.
            const std::string stats="Debug information\nState: connected\nSession: generated";
            std::vector<uint8_t> packet{2};packet.insert(packet.end(),stats.begin(),stats.end());
            view.cursor_update(packet);view.cursor_update({2});
        }
        DWORD available=0;require(PeekNamedPipe(reader,nullptr,0,nullptr,&available,nullptr)!=FALSE,"Cannot inspect generated events");
        require(available%8==0,"Unframed generated input");
        std::vector<uint8_t> events(available);DWORD read=0;
        require(ReadFile(reader,events.data(),available,&read,nullptr)!=FALSE && read==available,"Cannot read generated input");
        unsigned escapes=0,releases=0;
        for (size_t i=0;i<events.size();i+=8) {
            if (events[i]==8) ++escapes;
            if (events[i]==5) ++releases;
            require(!(events[i]==4 && events[i+5]==VK_ESCAPE),"Local Esc leaked to remote input");
        }
        require(escapes==2,"Esc did not exit exactly once in both input modes");
        require(releases>=1,"Held inputs were not released before local Esc");
        SetStdHandle(STD_ERROR_HANDLE,old_error);CloseHandle(writer);CloseHandle(reader);DestroyWindow(host);
        std::cout << "{\"layout_cases\":4,\"local_escape_modes\":2,\"remote_escape_events\":0,\"debug_toggle\":\"titlebar_only\"}\n";
    } catch (...) {
        SetStdHandle(STD_ERROR_HANDLE,old_error);CloseHandle(writer);CloseHandle(reader);DestroyWindow(host);throw;
    }
}
