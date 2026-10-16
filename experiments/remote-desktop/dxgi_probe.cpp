#include "dxgi_support.h"

static void capture(const Display& d, size_t index, unsigned seconds) {
    ComPtr<ID3D11Device> device;
    D3D_FEATURE_LEVEL level;
    check(D3D11CreateDevice(d.adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0, D3D11_SDK_VERSION,
        &device, &level, nullptr), "D3D11CreateDevice");
    ComPtr<IDXGIOutputDuplication> duplication;
    check(d.output->DuplicateOutput(device.Get(), &duplication), "DuplicateOutput");
    uint64_t acquired = 0, changed = 0, timeouts = 0, accumulated = 0;
    LONGLONG last_present = 0;
    D3D11_TEXTURE2D_DESC texture_desc{};
    double acquisition_ms = 0;
    auto start = Clock::now();
    while (Clock::now() - start < std::chrono::seconds(seconds)) {
        DXGI_OUTDUPL_FRAME_INFO info{};
        ComPtr<IDXGIResource> resource;
        auto before = Clock::now();
        HRESULT hr = duplication->AcquireNextFrame(100, &info, &resource);
        acquisition_ms += std::chrono::duration<double, std::milli>(Clock::now() - before).count();
        if (hr == DXGI_ERROR_WAIT_TIMEOUT) { ++timeouts; continue; }
        check(hr, "AcquireNextFrame");
        FrameLease lease{duplication.Get()};
        ComPtr<ID3D11Texture2D> texture;
        check(resource.As(&texture), "ID3D11Texture2D");
        texture->GetDesc(&texture_desc);
        ++acquired;
        if (info.LastPresentTime.QuadPart && info.LastPresentTime.QuadPart != last_present) {
            ++changed;
            last_present = info.LastPresentTime.QuadPart;
        }
        accumulated += info.AccumulatedFrames;
        // No Map, staging resource, CopyResource, encoder or renderer in this probe.
    }
    double elapsed = std::chrono::duration<double>(Clock::now() - start).count();
    std::cout << "{\"schema_version\":1,\"stage\":\"capture_only\",\"display\":";
    identify(d, index);
    std::cout << ",\"seconds\":" << elapsed << ",\"acquired_frames\":" << acquired
        << ",\"changed_frames\":" << changed << ",\"changed_capture_fps\":" << changed / elapsed
        << ",\"accumulated_frames\":" << accumulated << ",\"wait_timeouts\":" << timeouts
        << ",\"acquisition_wait_ms\":" << acquisition_ms
        << ",\"texture_width\":" << texture_desc.Width << ",\"texture_height\":" << texture_desc.Height
        << ",\"dxgi_format\":" << texture_desc.Format
        << ",\"probe_cpu_readbacks\":0,\"probe_gpu_copies\":0,\"probe_conversions\":0"
        << ",\"encoded\":false,\"rendered\":false,\"effective_display_fps\":null}\n";
}

static unsigned number(const std::string& text, unsigned maximum) {
    size_t end;
    if (text.empty() || text[0] == '-') throw std::runtime_error("invalid number");
    unsigned long value = std::stoul(text, &end);
    if (end != text.size() || value > maximum) throw std::runtime_error("number out of range");
    return static_cast<unsigned>(value);
}

int main(int argc, char** argv) {
    try {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        bool should_capture = false;
        unsigned selected = 0, seconds = 5;
        for (int i = 1; i < argc; ++i) {
            std::string arg = argv[i];
            if (arg == "--capture") should_capture = true;
            else if (arg == "--display" && i + 1 < argc) selected = number(argv[++i], 128);
            else if (arg == "--seconds" && i + 1 < argc) seconds = number(argv[++i], 60);
            else throw std::runtime_error("usage: dxgi-probe [--capture --display N --seconds 1..60]");
        }
        if (!seconds) throw std::runtime_error("seconds must be positive");
        auto found = displays();
        if (should_capture) {
            if (selected >= found.size()) throw std::runtime_error("display not attached in this session");
            capture(found[selected], selected, seconds);
        } else {
            std::cout << "{\"schema_version\":1,\"stage\":\"inventory\",\"displays\":[";
            for (size_t i = 0; i < found.size(); ++i) {
                if (i) std::cout << ',';
                identify(found[i], i);
            }
            std::cout << "]}\n";
        }
        return 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
