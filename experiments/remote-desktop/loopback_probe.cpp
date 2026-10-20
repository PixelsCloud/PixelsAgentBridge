// Standalone Windows media experiment. No production transport or application
// integration, no input injection, and no desktop pixel/bitstream persistence.
#include "nvenc_encoder.h"
#include "pattern_window.h"
#include "d3d_presenter.h"
#include <d3d11_1.h>
#include <dxgi1_4.h>
#include <psapi.h>
#include <algorithm>
#include <map>
#include <memory>

static unsigned number(const char* text, unsigned maximum) {
    std::string value(text);
    size_t end = 0;
    if (value.empty() || value[0] == '-') throw std::runtime_error("Invalid number");
    const auto n = std::stoul(value, &end);
    if (end != value.size() || n > maximum) throw std::runtime_error("Number out of range");
    return static_cast<unsigned>(n);
}

static double milliseconds(Clock::time_point start) {
    return std::chrono::duration<double, std::milli>(Clock::now() - start).count();
}

static void distribution(const char* name, std::vector<double> values) {
    if (values.empty()) throw std::runtime_error("Missing latency observations");
    std::sort(values.begin(), values.end());
    std::cout << ",\"" << name << "_p50_ms\":" << values[values.size() / 2]
        << ",\"" << name << "_p95_ms\":" << values[(values.size() - 1) * 95 / 100];
}

static double cpu_seconds() {
    FILETIME creation{}, exit{}, kernel{}, user{};
    if (!GetProcessTimes(GetCurrentProcess(), &creation, &exit, &kernel, &user))
        throw std::runtime_error("Cannot measure process CPU time");
    const auto ticks = [](const FILETIME& value) {
        return (static_cast<uint64_t>(value.dwHighDateTime) << 32) | value.dwLowDateTime;
    };
    return (ticks(kernel) + ticks(user)) / 10000000.0;
}

int main(int argc, char** argv) {
    try {
        unsigned selected = 0, seconds = 5, fps = 15;
        bool capture = false, animate = false, verify = false;
        for (int i = 1; i < argc; ++i) {
            const std::string arg(argv[i]);
            if (arg == "--capture") capture = true;
            else if (arg == "--animate") animate = true;
            else if (arg == "--verify-pixels") verify = true;
            else if (arg == "--display" && i + 1 < argc) selected = number(argv[++i], 128);
            else if (arg == "--seconds" && i + 1 < argc) seconds = number(argv[++i], 60);
            else if (arg == "--fps" && i + 1 < argc) fps = number(argv[++i], 60);
            else throw std::runtime_error("usage: loopback-probe [--capture [--animate] | --verify-pixels] [--display N] [--fps 1..60] [--seconds 1..60]");
        }
        if (!fps || !seconds) throw std::runtime_error("FPS and duration must be positive");
        if (animate && !capture) throw std::runtime_error("--animate requires --capture");
        if (verify && capture) throw std::runtime_error("Pixel verification is synthetic-only; real desktop readback refused");
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        MediaRuntime runtime;
        const auto found = displays();
        if (selected >= found.size()) throw std::runtime_error("Display not attached in this session");
        const auto& display = found[selected];
        if (capture && display.screen.Rotation != DXGI_MODE_ROTATION_IDENTITY)
            throw std::runtime_error("Rotated capture is not yet supported by this experiment");
        ComPtr<ID3D11Device> device;
        ComPtr<ID3D11DeviceContext> context;
        check(D3D11CreateDevice(display.adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, &context), "Create video D3D11 device");
        ComPtr<ID3D11DeviceContext1> context1;
        check(context.As(&context1), "D3D11.1 synthetic pattern support");
        ComPtr<IDXGIOutputDuplication> duplication;
        ComPtr<ID3D11Texture2D> synthetic;
        ComPtr<ID3D11RenderTargetView> target;
        std::unique_ptr<PatternWindow> pattern;
        UINT width = 1920, height = 1080;
        if (capture) {
            check(display.output->DuplicateOutput(device.Get(), &duplication), "DuplicateOutput");
            DXGI_OUTDUPL_DESC description{};
            duplication->GetDesc(&description);
            width = description.ModeDesc.Width; height = description.ModeDesc.Height;
            if (animate) {
                pattern = std::make_unique<PatternWindow>();
                pattern->initialize(device.Get(), display.screen.DesktopCoordinates);
            }
        } else {
            D3D11_TEXTURE2D_DESC desc{};
            desc.Width = width; desc.Height = height;
            desc.MipLevels = desc.ArraySize = desc.SampleDesc.Count = 1;
            desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
            desc.BindFlags = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
            check(device->CreateTexture2D(&desc, nullptr, &synthetic), "Create synthetic GPU texture");
            check(device->CreateRenderTargetView(synthetic.Get(), nullptr, &target), "Create synthetic render target");
        }
        Encoder encoder;
        encoder.initialize(device.Get(), width, height, fps);
        HardwareDecoder decoder(device.Get(), width, height, fps);
        NativePresenter presenter;
        presenter.initialize(device.Get(), width, height, fps, display.screen.DesktopCoordinates, capture, verify);
        // All initialization is outside the measured interval. First-frame
        // decode/display cold-start latency remains in the measurements.
        uint64_t encoded = 0, decoded = 0, bytes = 0, pattern_updates = 0;
        size_t maximum_pending = 0;
        LONGLONG last_present = 0;
        std::vector<double> encode_times, decode_times, render_times, total_times;
        struct Pending { uint64_t index; Clock::time_point source, submitted; };
        std::map<LONGLONG, Pending> pending;
        std::vector<uint8_t> packet;
        ComPtr<IDXGIAdapter3> budget_adapter;
        check(display.adapter.As(&budget_adapter), "Adapter memory accounting");
        uint64_t peak_video_memory = 0;
        auto consume = [&](DecodedFrame& frame) {
            auto entry = pending.find(frame.timestamp);
            if (entry == pending.end() || entry->second.index != decoded)
                throw std::runtime_error("Missing, duplicate, or reordered decoded timestamp");
            decode_times.push_back(milliseconds(entry->second.submitted));
            const auto before = Clock::now();
            presenter.render(frame, entry->second.index);
            render_times.push_back(milliseconds(before));
            total_times.push_back(milliseconds(entry->second.source));
            ++decoded;
            pending.erase(entry);
            DXGI_QUERY_VIDEO_MEMORY_INFO memory{};
            check(budget_adapter->QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &memory), "GPU memory accounting");
            peak_video_memory = (std::max)(peak_video_memory, memory.CurrentUsage);
        };
        const double cpu_start = cpu_seconds();
        const auto start = Clock::now();
        auto next = start;
        while (Clock::now() - start < std::chrono::seconds(seconds)) {
            std::this_thread::sleep_until(next);
            if (Clock::now() - start >= std::chrono::seconds(seconds)) break;
            presenter.pump();
            const auto source_time = Clock::now();
            ComPtr<ID3D11Texture2D> texture;
            // Release capture immediately after encoder consumption, before
            // decode/display waits. Retaining it would stall desktop updates.
            {
                FrameLease lease{nullptr};
                if (capture) {
                    if (pattern) pattern->draw(context.Get(), pattern_updates++);
                    DXGI_OUTDUPL_FRAME_INFO info{};
                    ComPtr<IDXGIResource> resource;
                    const auto hr = duplication->AcquireNextFrame(100, &info, &resource);
                    if (hr == DXGI_ERROR_WAIT_TIMEOUT) continue;
                    check(hr, "AcquireNextFrame");
                    lease.duplication = duplication.Get();
                    check(resource.As(&texture), "Captured GPU texture");
                    if (!info.LastPresentTime.QuadPart || info.LastPresentTime.QuadPart == last_present) continue;
                    last_present = info.LastPresentTime.QuadPart;
                } else {
                    for (unsigned quadrant = 0; quadrant < 4; ++quadrant) {
                        const LONG x = static_cast<LONG>((quadrant % 2) * width / 2);
                        const LONG y = static_cast<LONG>((quadrant / 2) * height / 2);
                        const D3D11_RECT rect{x, y, x + static_cast<LONG>(width / 2), y + static_cast<LONG>(height / 2)};
                        const auto color = test_color(encoded, quadrant);
                        context1->ClearView(target.Get(), color.data(), &rect, 1);
                    }
                    context->Flush();
                    texture = synthetic;
                }
                const auto before = Clock::now();
                bytes += encoder.encode(texture.Get(), encoded, nullptr, &packet);
                encode_times.push_back(milliseconds(before));
            }
            texture.Reset();
            const LONGLONG timestamp = static_cast<LONGLONG>(encoded) * 10000000 / fps;
            pending.emplace(timestamp, Pending{encoded, source_time, Clock::now()});
            maximum_pending = (std::max)(maximum_pending, pending.size());
            if (pending.size() > 4) throw std::runtime_error("Decode latency exceeded bounded experiment queue");
            decoder.submit(packet, timestamp);
            ++encoded;
            decoder.receive(consume);
            next = (std::max)(next + std::chrono::microseconds(1000000 / fps), Clock::now());
        }
        decoder.drain(consume);
        const double elapsed = std::chrono::duration<double>(Clock::now() - start).count();
        const double cpu_elapsed = cpu_seconds() - cpu_start;
        PROCESS_MEMORY_COUNTERS_EX memory{};
        memory.cb = sizeof(memory);
        if (!GetProcessMemoryInfo(GetCurrentProcess(), reinterpret_cast<PROCESS_MEMORY_COUNTERS*>(&memory), sizeof(memory)))
            throw std::runtime_error("Cannot measure process memory");
        if (!encoded || encoded != decoded || decoded != presenter.displayed || !pending.empty())
            throw std::runtime_error("Encode/decode/display frame counts differ");
        std::cout << "{\"schema_version\":1,\"stage\":\"h264_gpu_local_loopback\",\"source\":\""
            << (capture ? "dxgi_desktop" : "synthetic_gpu_quadrants") << "\",\"display\":";
        identify(display, selected);
        std::cout << ",\"width\":" << width << ",\"height\":" << height
            << ",\"window_client_width\":960,\"window_client_height\":540"
            << ",\"target_fps\":" << fps << ",\"seconds\":" << elapsed
            << ",\"encoded_frames\":" << encoded << ",\"decoded_frames\":" << decoded
            << ",\"present_submissions\":" << presenter.submitted
            << ",\"display_confirmed_frames\":" << presenter.displayed
            << ",\"effective_display_fps\":" << presenter.displayed / elapsed
            << ",\"encoded_bytes\":" << bytes << ",\"maximum_pending_frames\":" << maximum_pending
            << ",\"test_pattern_updates\":" << pattern_updates
            << ",\"process_cpu_seconds\":" << cpu_elapsed
            << ",\"process_cpu_core_equivalents\":" << cpu_elapsed / elapsed
            << ",\"process_peak_working_set_bytes\":" << memory.PeakWorkingSetSize
            << ",\"process_private_bytes_at_end\":" << memory.PrivateUsage
            << ",\"process_peak_local_video_memory_bytes\":" << peak_video_memory;
        distribution("encode", encode_times);
        distribution("decode_output", decode_times);
        distribution("render_display_observed", render_times);
        distribution("source_to_display_observed", total_times);
        std::cout << ",\"decoder\":\"system_h264_d3d11_dxva\",\"decoder_gpu_surfaces_required\":true"
            << ",\"render_path\":\"nv12_video_processor_to_bgra_flip_swapchain\""
            << ",\"color_matrix\":\"bt709\",\"video_range\":\"limited\",\"rgb_range\":\"full\""
            << ",\"display_confirmation\":\"dxgi_frame_statistics_each_present_id\""
            << ",\"pixel_verification\":" << (verify ? "true" : "false")
            << ",\"test_pixel_readbacks\":" << presenter.pixel_checks
            << ",\"test_max_rgb_error\":" << presenter.max_color_error
            << ",\"test_pixel_gpu_copies\":" << presenter.pixel_checks
            << ",\"media_pixel_cpu_readbacks\":0,\"media_gpu_texture_copies\":0"
            << ",\"gpu_color_conversion_passes\":" << decoded
            << ",\"frame_statistics_disjoint\":" << presenter.stats_disjoint
            << ",\"driver_internal_copies_known\":false,\"network_tested\":false}\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
