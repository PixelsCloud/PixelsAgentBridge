// Independent experiment using NVENC's public API; not copied SDK sample code.
// Reference: pinned hwcodec nv_encode.cpp; see sources.toml and README.md.
#include "dxgi_support.h"
#include "nvenc_encoder.h"
#include "pattern_window.h"
#include <algorithm>
#include <fstream>
#include <memory>
#include <thread>


static unsigned number(const char* text, unsigned maximum) {
    std::string value(text);
    size_t end = 0;
    if (value.empty() || value[0] == '-') throw std::runtime_error("Invalid number");
    const auto n = std::stoul(value, &end);
    if (end != value.size() || n > maximum) throw std::runtime_error("Number out of range");
    return static_cast<unsigned>(n);
}


int main(int argc, char** argv) {
    try {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        unsigned selected = 0, seconds = 5, fps = 15;
        bool desktop = false, animate = false;
        std::string destination;
        for (int i = 1; i < argc; ++i) {
            const std::string arg(argv[i]);
            if (arg == "--capture") desktop = true;
            else if (arg == "--animate") animate = true;
            else if (arg == "--display" && i + 1 < argc) selected = number(argv[++i], 128);
            else if (arg == "--seconds" && i + 1 < argc) seconds = number(argv[++i], 60);
            else if (arg == "--fps" && i + 1 < argc) fps = number(argv[++i], 60);
            else if (arg == "--output" && i + 1 < argc) destination = argv[++i];
            else throw std::runtime_error("usage: nvenc-probe [--capture [--animate]] [--display N] [--fps 1..60] [--seconds 1..60] [--output synthetic.h264]");
        }
        if (!fps || !seconds) throw std::runtime_error("FPS and duration must be positive");
        if (desktop && !destination.empty()) throw std::runtime_error("Saving real desktop video is disabled in this experiment");
        if (animate && !desktop) throw std::runtime_error("--animate requires --capture");
        auto found = displays();
        if (selected >= found.size()) throw std::runtime_error("Display not attached in this session");
        const auto& d = found[selected];
        ComPtr<ID3D11Device> device;
        ComPtr<ID3D11DeviceContext> context;
        check(D3D11CreateDevice(d.adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0, D3D11_SDK_VERSION,
            &device, nullptr, &context), "D3D11CreateDevice");
        ComPtr<IDXGIOutputDuplication> duplication;
        ComPtr<ID3D11Texture2D> synthetic;
        ComPtr<ID3D11RenderTargetView> render_target;
        std::unique_ptr<PatternWindow> pattern;
        if (animate) {
            pattern = std::make_unique<PatternWindow>();
            pattern->initialize(device.Get(), d.screen.DesktopCoordinates);
        }
        if (desktop) check(d.output->DuplicateOutput(device.Get(), &duplication), "DuplicateOutput");
        else {
            D3D11_TEXTURE2D_DESC desc{};
            desc.Width = 1920; desc.Height = 1080;
            desc.MipLevels = desc.ArraySize = desc.SampleDesc.Count = 1;
            desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
            desc.Usage = D3D11_USAGE_DEFAULT;
            desc.BindFlags = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
            check(device->CreateTexture2D(&desc, nullptr, &synthetic), "CreateSyntheticTexture");
            check(device->CreateRenderTargetView(synthetic.Get(), nullptr, &render_target), "CreateRenderTargetView");
        }
        std::ofstream output;
        if (!destination.empty()) {
            if (GetFileAttributesA(destination.c_str()) != INVALID_FILE_ATTRIBUTES)
                throw std::runtime_error("Output already exists; choose a new filename");
            output.open(destination, std::ios::binary);
            if (!output) throw std::runtime_error("Cannot open output file");
        }
        std::unique_ptr<Encoder> encoder;
        std::vector<double> timings;
        uint64_t bytes = 0, frames = 0, changed = 0, pattern_updates = 0;
        LONGLONG last_present = 0;
        UINT width = 0, height = 0;
        const auto start = Clock::now();
        auto next = start;
        while (Clock::now() - start < std::chrono::seconds(seconds)) {
            std::this_thread::sleep_until(next);
            if (Clock::now() - start >= std::chrono::seconds(seconds)) break;
            ComPtr<ID3D11Texture2D> texture;
            FrameLease lease{nullptr};
            if (desktop) {
                if (pattern) pattern->draw(context.Get(), pattern_updates++);
                DXGI_OUTDUPL_FRAME_INFO info{};
                ComPtr<IDXGIResource> resource;
                const auto hr = duplication->AcquireNextFrame(100, &info, &resource);
                if (hr == DXGI_ERROR_WAIT_TIMEOUT) continue;
                check(hr, "AcquireNextFrame");
                lease.duplication = duplication.Get();
                check(resource.As(&texture), "CaptureTexture");
                if (!info.LastPresentTime.QuadPart || info.LastPresentTime.QuadPart == last_present) continue;
                last_present = info.LastPresentTime.QuadPart;
            } else {
                const float color[] = {float(frames % 251) / 250.0f, float((frames * 7) % 251) / 250.0f, float((frames * 19) % 251) / 250.0f, 1.0f};
                context->ClearRenderTargetView(render_target.Get(), color);
                context->Flush();
                texture = synthetic;
            }
            ++changed;
            if (!encoder) {
                D3D11_TEXTURE2D_DESC desc{};
                texture->GetDesc(&desc);
                width = desc.Width; height = desc.Height;
                encoder = std::make_unique<Encoder>();
                encoder->initialize(device.Get(), width, height, fps);
            }
            const auto before = Clock::now();
            bytes += encoder->encode(texture.Get(), frames, output.is_open() ? &output : nullptr);
            timings.push_back(std::chrono::duration<double, std::milli>(Clock::now() - before).count());
            ++frames;
            next = std::max(next + std::chrono::microseconds(1000000 / fps), Clock::now());
        }
        if (!frames) throw std::runtime_error("No changed frames encoded");
        const double elapsed = std::chrono::duration<double>(Clock::now() - start).count();
        std::sort(timings.begin(), timings.end());
        std::cout << "{\"schema_version\":1,\"stage\":\"direct_texture_h264_encode\",\"source\":\""
            << (desktop ? "dxgi_desktop" : "synthetic_gpu") << "\",\"display\":";
        identify(d, selected);
        std::cout << ",\"width\":" << width << ",\"height\":" << height
            << ",\"target_fps\":" << fps << ",\"seconds\":" << elapsed
            << ",\"changed_inputs\":" << changed << ",\"encoded_frames\":" << frames
            << ",\"encoded_fps\":" << frames / elapsed << ",\"encoded_bytes\":" << bytes
            << ",\"encode_p50_ms\":" << timings[timings.size() / 2]
            << ",\"encode_p95_ms\":" << timings[(timings.size() - 1) * 95 / 100]
            << ",\"test_pattern_updates\":" << pattern_updates
            << ",\"probe_pixel_cpu_readbacks\":0,\"probe_gpu_copies\":0,\"probe_conversions\":0"
            << ",\"driver_internal_copies_known\":false,\"decoded\":false,\"rendered\":false,\"effective_display_fps\":null}\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
