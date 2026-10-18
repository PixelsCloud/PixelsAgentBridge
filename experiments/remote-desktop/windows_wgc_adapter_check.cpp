// Opt-in check of the production WGC adapter on an owned four-color fixture.
// Only the diagnostic reads four GPU pixels back; production never maps them.
#include "../../crates/desktop-media/native/windows/wgc_capture.h"

int main(int argc, char** argv) {
    if (argc != 2 || std::string(argv[1]) != "--check-generated") return 2;
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    try {
        auto found = displays(); const Display* selected = nullptr;
        for (const auto& d : found) {
            const auto& r = d.screen.DesktopCoordinates;
            if (r.left == -1080 && r.top == 0 && r.right == 0 && r.bottom == 1920
                && d.screen.Rotation == DXGI_MODE_ROTATION_ROTATE90) {
                if (selected) throw std::runtime_error("Ambiguous fixture display");
                selected = &d;
            }
        }
        if (!selected) throw std::runtime_error("Expected generated portrait display");
        const COLORREF colors[] = {RGB(180,40,40), RGB(40,180,40), RGB(40,40,180), RGB(180,180,40)};
        HDC dc = GetDC(nullptr);
        if (!dc) throw std::runtime_error("GDI DC unavailable");
        bool generated = false;
        const auto fixture_deadline = Clock::now() + std::chrono::seconds(5);
        while (!generated && Clock::now() < fixture_deadline) {
            generated = true;
            for (unsigned i = 0; i < 4; ++i)
                generated &= GetPixel(dc, -1080 + 1080 * (i%2 ? 3:1)/4, 1920 * (i/2 ? 3:1)/4) == colors[i];
            if (!generated) Sleep(50);
        }
        ReleaseDC(nullptr, dc);
        if (!generated) throw std::runtime_error("Generated surface not confirmed");
        WgcRuntime runtime;
        ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context;
        check(D3D11CreateDevice(selected->adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, &context), "device");
        Display stale = *selected; ++stale.screen.DesktopCoordinates.right;
        bool rejected = false;
        std::cerr << "checking stale geometry\n" << std::flush;
        try { WgcCapture invalid(device.Get(), stale, 15); }
        catch (const std::runtime_error& e) { rejected = std::string(e.what()) == "Screen changed; refresh generation"; }
        if (!rejected) throw std::runtime_error("Stale geometry accepted");
        std::cerr << "stale geometry rejected; opening capture\n" << std::flush;
        WgcCapture capture(device.Get(), *selected, 15);
        std::cerr << "capture opened; acquiring frame\n" << std::flush;
        auto texture = capture.next(5000);
        std::cerr << "acquisition returned\n" << std::flush;
        if (!texture) throw std::runtime_error("WGC adapter timeout");
        D3D11_TEXTURE2D_DESC desc{}; texture->GetDesc(&desc);
        if (desc.Width != 1920 || desc.Height != 1080) throw std::runtime_error("Wrong native dimensions");
        desc.Usage = D3D11_USAGE_STAGING; desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ; desc.MiscFlags = 0;
        ComPtr<ID3D11Texture2D> staging;
        check(device->CreateTexture2D(&desc, nullptr, &staging), "staging");
        context->CopyResource(staging.Get(), texture.Get());
        D3D11_MAPPED_SUBRESOURCE mapped{};
        check(context->Map(staging.Get(), 0, D3D11_MAP_READ, 0, &mapped), "map");
        const unsigned expected[] = {1, 3, 0, 2}; // Undo clockwise 90 degrees.
        bool correct = true;
        std::cout << "{\"stale_geometry_rejected\":true,\"width\":1920,\"height\":1080,\"colors\":[";
        for (unsigned i = 0; i < 4; ++i) {
            const auto* p = static_cast<const uint8_t*>(mapped.pData)
                + size_t(1080*(i/2 ? 3:1)/4)*mapped.RowPitch + (1920*(i%2 ? 3:1)/4)*4;
            std::cout << (i ? ",":"") << '[' << unsigned(p[2]) << ',' << unsigned(p[1]) << ',' << unsigned(p[0]) << ']';
            correct &= RGB(p[2],p[1],p[0]) == colors[expected[i]];
        }
        context->Unmap(staging.Get(), 0);
        std::cout << "],\"correct\":" << (correct ? "true":"false") << "}\n" << std::flush;
        return correct ? 0:1;
    } catch (const std::exception& e) { std::cerr << e.what() << '\n'; return 1; }
}
