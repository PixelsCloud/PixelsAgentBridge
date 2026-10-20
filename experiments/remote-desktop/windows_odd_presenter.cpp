// Generated NV12 pixels exercise the production GPU presenter without capture,
// network, decoder or user content. Every window belongs to this process.
#include "../../crates/desktop-media/native/windows/d3d_presenter.h"

int main() {
    try {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        MediaRuntime runtime;
        ComPtr<ID3D11Device> device;
        check(D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, nullptr), "Create test device");
        bool passed = true;
        for (auto size : {std::pair<UINT, UINT>{664, 474}, {663, 474}, {664, 473}, {663, 473}}) {
            const auto [width, height] = size;
            const PabVideoGeometry geometry(width, height);
            std::vector<uint8_t> pixels(geometry.nv12_bytes(), 128);
            for (UINT y = 0; y < geometry.coded_height; ++y)
                for (UINT x = 0; x < geometry.coded_width; ++x)
                    pixels[y * geometry.coded_width + x] =
                        (!x || !y || x >= width - 1 || y >= height - 1) ? 235 : 74;
            D3D11_TEXTURE2D_DESC description{};
            description.Width = geometry.coded_width; description.Height = geometry.coded_height;
            description.MipLevels = description.ArraySize = description.SampleDesc.Count = 1;
            description.Format = DXGI_FORMAT_NV12; description.Usage = D3D11_USAGE_DEFAULT;
            description.BindFlags = D3D11_BIND_DECODER;
            D3D11_SUBRESOURCE_DATA initial{pixels.data(), geometry.coded_width, 0};
            DecodedFrame frame;
            check(device->CreateTexture2D(&description, &initial, &frame.texture), "Create generated NV12");
            check(MFCreateSample(&frame.sample), "Create retained test sample");
            for (unsigned rotation = 0; rotation < 4; ++rotation) {
                NativePresenter presenter;
                presenter.initialize(device.Get(), width, height, 15, RECT{}, false, false, nullptr, rotation);
                HWND window = FindWindowW(L"PixelsRemoteDesktopSurface", nullptr);
                DWORD pid = 0; GetWindowThreadProcessId(window, &pid);
                if (pid != GetCurrentProcessId()) throw std::runtime_error("Window ownership mismatch");
                const LONG w = rotation % 2 ? height : width, h = rotation % 2 ? width : height;
                RECT outer{}, client{}; GetWindowRect(window, &outer); GetClientRect(window, &client);
                if (!SetWindowPos(window, HWND_TOPMOST, 70, 70,
                    w + outer.right - outer.left - client.right,
                    h + outer.bottom - outer.top - client.bottom, SWP_NOACTIVATE))
                    throw std::runtime_error("Resize test window");
                presenter.pump(); presenter.render(frame, 0);
                const auto deadline = Clock::now() + std::chrono::milliseconds(600);
                while (Clock::now() < deadline) { presenter.pump(); Sleep(5); }
                GetClientRect(window, &client);
                if (client.right != w || client.bottom != h) throw std::runtime_error("Unexpected client size");
                POINT origin{}; ClientToScreen(window, &origin);
                HDC dc = GetDC(nullptr);
                const POINT points[]{{w/2,0},{w/2,h-1},{0,h/2},{w-1,h/2},
                    {w-1,h-1},{1,h/2},{w-2,h/2},{w/2,1},{w/2,h-2},{w/2,h/2}};
                bool valid = true;
                std::cout << "{\"width\":" << width << ",\"height\":" << height
                    << ",\"rotation\":" << rotation << ",\"red\":[";
                for (unsigned i = 0; i < std::size(points); ++i) {
                    const auto color = GetPixel(dc, origin.x + points[i].x, origin.y + points[i].y);
                    const unsigned red = GetRValue(color);
                    valid &= color != CLR_INVALID && (i < 5 ? red > 220 : red > 40 && red < 100);
                    std::cout << (i ? "," : "") << red;
                }
                ReleaseDC(nullptr, dc);
                std::cout << "],\"passed\":" << (valid ? "true" : "false") << "}\n";
                passed &= valid;
            }
        }
        return passed ? 0 : 1;
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 2; }
}
