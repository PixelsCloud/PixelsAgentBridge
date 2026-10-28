// Isolated native media worker. Compressed H.264 and bounded cursor snapshots
// cross anonymous pipes; cursor sampling never reads back the desktop surface.
#include "nvenc_encoder.h"
#include "pattern_window.h"
#include "d3d_presenter.h"
#include "software_codec.h"
#include "wgc_capture.h"
#include "capture_events.h"
#include "media_wait_fixture.h"
#include <d3d11_1.h>
#include <shellscalingapi.h>
#include <algorithm>
#include <memory>

static constexpr uint32_t magic = 0x31424150; // PAB1, little endian local IPC
static constexpr uint32_t max_packet = 16 * 1024 * 1024;

static void read_bytes(void* data, uint32_t size, NativePresenter* viewer = nullptr,
    const std::function<void()>& idle = {}) {
    auto* cursor = static_cast<uint8_t*>(data);
    while (size) {
        if (viewer) viewer->pump();
        DWORD available = 0;
        if (!PeekNamedPipe(GetStdHandle(STD_INPUT_HANDLE), nullptr, 0, nullptr, &available, nullptr))
            throw std::runtime_error("Parent pipe ended");
        if (!available) { if (idle) idle(); media_wait::sleep(std::chrono::milliseconds(2)); continue; }
        DWORD read = 0;
        if (!ReadFile(GetStdHandle(STD_INPUT_HANDLE), cursor, (std::min)(size, static_cast<uint32_t>(available)), &read, nullptr) || !read)
            throw std::runtime_error("Parent pipe read failed");
        cursor += read; size -= read;
    }
}
static void write_bytes(const void* data, uint32_t size) {
    const auto* cursor = static_cast<const uint8_t*>(data);
    while (size) {
        DWORD written = 0;
        if (!WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), cursor, size, &written, nullptr) || !written)
            throw std::runtime_error("Parent pipe write failed");
        cursor += written; size -= written;
    }
}
template<class T> static T read_value(NativePresenter* viewer = nullptr,
    const std::function<void()>& idle = {}) {
    T value{}; read_bytes(&value, sizeof(value), viewer, idle); return value;
}
template<class T> static void write_value(T value) { write_bytes(&value, sizeof(value)); }
static unsigned number(const char* value, unsigned maximum) {
    const std::string text(value);
    size_t end = 0;
    if (text.empty() || text[0] == '-') throw std::runtime_error("Invalid number");
    const auto n = std::stoul(text, &end);
    if (end != text.size() || n == 0 || n > maximum) throw std::runtime_error("Number outside bounds");
    return static_cast<unsigned>(n);
}
static std::string screen_id(const Display& d) {
    std::string name;
    for (const wchar_t* c = d.screen.DeviceName; *c; ++c) name += static_cast<char>(*c);
    return std::to_string(d.gpu.AdapterLuid.HighPart) + ":" + std::to_string(d.gpu.AdapterLuid.LowPart) + ":" + name;
}
static void inventory(const std::vector<Display>& found) {
    std::cout << '[';
    bool first = true;
    for (const auto& d : found) {
        if (!first) std::cout << ',';
        first = false;
        const auto& r = d.screen.DesktopCoordinates;
        const auto id = screen_id(d);
        const std::wstring wide(id.begin(), id.end());
        const bool rotated = d.screen.Rotation == DXGI_MODE_ROTATION_ROTATE90 || d.screen.Rotation == DXGI_MODE_ROTATION_ROTATE270;
        const char* rotation = d.screen.Rotation == DXGI_MODE_ROTATION_ROTATE90 ? "clockwise90" :
            d.screen.Rotation == DXGI_MODE_ROTATION_ROTATE180 ? "clockwise180" :
            d.screen.Rotation == DXGI_MODE_ROTATION_ROTATE270 ? "clockwise270" : "identity";
        MONITORINFO monitor{}; monitor.cbSize = sizeof(monitor);
        if (!GetMonitorInfoW(d.screen.Monitor, &monitor)) throw std::runtime_error("Monitor info failed");
        std::cout << "{\"id\":" << quoted(wide.c_str()) << ",\"name\":" << quoted(d.screen.DeviceName)
            << ",\"x\":" << r.left << ",\"y\":" << r.top
            << ",\"desktop_width\":" << r.right-r.left << ",\"desktop_height\":" << r.bottom-r.top
            << ",\"pixel_width\":" << (rotated ? r.bottom-r.top : r.right-r.left)
            << ",\"pixel_height\":" << (rotated ? r.right-r.left : r.bottom-r.top)
            << ",\"scale_milli\":" << d.dpi_x * 1000 / 96 << ",\"rotation\":\"" << rotation
            << "\",\"primary\":" << ((monitor.dwFlags & MONITORINFOF_PRIMARY) ? "true" : "false") << '}';
    }
    std::cout << "]\n";
}
#include "input_worker.h"
#include "user_capture.h"
#include "input_fixture.h"
static void validate_wgc_desktop(const Display& selected);
#include "cursor_worker.h"
#include "cursor_fixture.h"
#include "win32_cursor_fixture.h"
#include "local_cursor_fixture.h"
#include "cursor_overlay_fixture.h"
#include "viewer_chrome_fixture.h"
static bool idr(const std::vector<uint8_t>& bytes) {
    for (size_t i = 0; i + 4 < bytes.size(); ++i) {
        if (!bytes[i] && !bytes[i+1] && bytes[i+2] == 1 && (bytes[i+3] & 31) == 5) return true;
    }
    return false;
}
static void validate_display_layout(const Display& selected) {
    for (const auto& current : displays()) if (screen_id(current) == screen_id(selected)) {
        if (same_display_layout(selected, current)) return;
        break;
    }
    throw std::runtime_error("Screen changed; refresh generation");
}
static void validate_wgc_desktop(const Display& selected) {
    // WGC is only a fallback on the same ordinary desktop. A lock, session
    // switch or monitor change must still go through generation recovery.
    HDESK desktop = OpenInputDesktop(0, FALSE, DESKTOP_READOBJECTS);
    if (!desktop) throw std::runtime_error("Active capture desktop unavailable");
    std::wstring active;
    try { active = desktop_name(desktop); } catch (...) { CloseDesktop(desktop); throw; }
    CloseDesktop(desktop);
    if (active != L"Default" || desktop_name(GetThreadDesktop(GetCurrentThreadId())) != active)
        throw std::runtime_error("Capture desktop changed; renew generation");
    validate_display_layout(selected);
}
static void capture(const Display& display, unsigned fps, const std::string& mode, bool force_software, bool fail_encoder,
    const std::string& capture_token = {}, const std::string& proof_log = {}) {
    const bool synthetic = mode == "synthetic";
    const bool proof = mode == "desktop-proof";
    if (!synthetic && mode != "desktop" && mode != "animate" && !proof) throw std::runtime_error("Unknown source mode");
    if (proof != !proof_log.empty()) throw std::runtime_error("Explicit proof log required only for synthetic pattern checking");
    std::ofstream proof_output;
    if (proof) { proof_output.open(proof_log,std::ios::out|std::ios::trunc); if(!proof_output) throw std::runtime_error("Cannot open own proof log"); }
    std::unique_ptr<WgcRuntime> wgc_runtime; // Released after the shared device.
    std::unique_ptr<CaptureEvents> coordination;
    if (!capture_token.empty()) coordination = std::make_unique<CaptureEvents>(capture_token, true);
    ComPtr<ID3D11Device> device;
    ComPtr<ID3D11DeviceContext> context;
    check(D3D11CreateDevice(display.adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, &context), "Capture device");
    ComPtr<IDXGIOutputDuplication> duplication;
    std::unique_ptr<WgcCapture> wgc;
    ComPtr<ID3D11Texture2D> texture;
    ComPtr<ID3D11RenderTargetView> target;
    ComPtr<ID3D11DeviceContext1> context1;
    std::unique_ptr<PatternWindow> pattern;
    const auto& rect = display.screen.DesktopCoordinates;
    const bool rotated = display.screen.Rotation == DXGI_MODE_ROTATION_ROTATE90 || display.screen.Rotation == DXGI_MODE_ROTATION_ROTATE270;
    UINT width = rotated ? rect.bottom-rect.top : rect.right-rect.left;
    UINT height = rotated ? rect.right-rect.left : rect.bottom-rect.top;
    if (!synthetic) {
        check(display.output->DuplicateOutput(device.Get(), &duplication), "DuplicateOutput");
        DXGI_OUTDUPL_DESC description{}; duplication->GetDesc(&description);
        // DXGI's mode is in desktop orientation; acquired textures remain in
        // native orientation. Comparing the mode to encoder dimensions rejects
        // every 90/270-degree display even when its layout has not changed.
        if (description.ModeDesc.Width != UINT(rect.right-rect.left)
            || description.ModeDesc.Height != UINT(rect.bottom-rect.top)
            || description.Rotation != display.screen.Rotation)
            throw std::runtime_error("Screen changed; refresh generation");
        if (mode == "animate") {
            pattern = std::make_unique<PatternWindow>(); pattern->initialize(device.Get(), rect);
        }
    } else {
        check(context.As(&context1), "Synthetic D3D11.1 context");
        D3D11_TEXTURE2D_DESC desc{};
        desc.Width = width; desc.Height = height;
        desc.MipLevels = desc.ArraySize = desc.SampleDesc.Count = 1;
        desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.BindFlags = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
        check(device->CreateTexture2D(&desc, nullptr, &texture), "Synthetic texture");
        check(device->CreateRenderTargetView(texture.Get(), nullptr, &target), "Synthetic target");
    }
    CaptureEncoder encoder(device.Get(), width, height, fps, force_software, fail_encoder);
    std::unique_ptr<pattern_proof::GPU> proof_gpu;
    if(proof) proof_gpu=std::make_unique<pattern_proof::GPU>(device.Get(),context.Get());
    const auto switch_wgc = [&] {
        if (wgc) return;
        if (synthetic) throw std::runtime_error("Synthetic capture cannot change backend");
        const char* stage = "desktop validation";
        try {
            validate_wgc_desktop(display);
            duplication.Reset();
            stage = "runtime initialization";
            wgc_runtime = std::make_unique<WgcRuntime>();
            stage = "capture initialization";
            wgc = std::make_unique<WgcCapture>(device.Get(), display, fps);
            stage = "cursor coordination";
            if (coordination) coordination->activate();
        } catch (const std::exception& error) {
            throw std::runtime_error(std::string("WGC fallback ") + stage + ": " + error.what());
        }
        std::cerr << "media capture=wgc_gpu readback_per_frame=0\n";
    };
    auto next_layout = Clock::now();
    const auto coordinate = [&] {
        // DPI can change without invalidating a DXGI duplication. Check even
        // on a static desktop or while waiting for video credit. This guard
        // only checks layout; WGC's Default-desktop restriction must not be
        // applied to DXGI capture of the secure login desktop.
        if (!synthetic && Clock::now() >= next_layout) {
            validate_display_layout(display);
            next_layout = Clock::now() + std::chrono::milliseconds(100);
        }
        if (coordination && coordination->requested()) switch_wgc();
    };
    std::cerr << "media encoder=" << encoder.name() << " readback_per_frame=" << (encoder.software() ? 1 : 0) << '\n';
    write_value(magic); write_value(width); write_value(height);
    const auto start = Clock::now(); auto next = start;
    uint64_t index = 0;
    LONGLONG last_present = 0;
    while (true) {
        // Cursor recovery must also work while video has no consumption credit.
        const auto command = read_value<uint8_t>(nullptr, coordinate);
        if (command == 0) { if (mode != "desktop") encoder.report(); return; }
        if (command == 2) {
            const bool applied = encoder.budget(read_value<uint32_t>(nullptr, coordinate));
            write_value(applied ? magic : magic + 1);
            continue;
        }
        if (command != 1 && command != 3 && command != 4) throw std::runtime_error("Invalid capture command");
        const auto capture_started = Clock::now();
        media_wait::until(next);
        std::vector<uint8_t> packet;
        auto sampled = Clock::now();
        Clock::time_point encode_started;
        uint64_t encode_us = 0;
        pattern_proof::Result proof_result;
        {
            FrameLease lease{nullptr};
            if (synthetic) {
                for (unsigned q = 0; q < 4; ++q) {
                    const LONG x = static_cast<LONG>((q % 2) * width / 2), y = static_cast<LONG>((q / 2) * height / 2);
                    const D3D11_RECT region{x,y,x+static_cast<LONG>(width/2),y+static_cast<LONG>(height/2)};
                    const auto color = test_color(index,q);
                    context1->ClearView(target.Get(), color.data(), &region, 1);
                }
                context->Flush();
            } else {
                while (!lease.duplication) {
                    DWORD available = 0;
                    if (!PeekNamedPipe(GetStdHandle(STD_INPUT_HANDLE), nullptr, 0, nullptr, &available, nullptr))
                        throw std::runtime_error("Parent ended while waiting for desktop changes");
                    if (pattern) pattern->draw(context.Get(), index);
                    coordinate();
                    if (wgc) {
                        validate_wgc_desktop(display);
                        texture = wgc->next(100);
                        if (!texture) continue;
                        // Do not encode a frame obtained across a desktop or
                        // geometry transition during the bounded wait.
                        validate_wgc_desktop(display);
                        break;
                    }
                    DXGI_OUTDUPL_FRAME_INFO info{}; ComPtr<IDXGIResource> resource;
                    const auto hr = duplication->AcquireNextFrame(100, &info, &resource);
                    if (hr == DXGI_ERROR_WAIT_TIMEOUT) continue;
                    if (hr == DXGI_ERROR_ACCESS_LOST) {
                        switch_wgc();
                        continue;
                    }
                    check(hr, "AcquireNextFrame");
                    if (!info.LastPresentTime.QuadPart || info.LastPresentTime.QuadPart == last_present) {
                        duplication->ReleaseFrame(); continue;
                    }
                    lease.duplication = duplication.Get(); last_present = info.LastPresentTime.QuadPart;
                    check(resource.As(&texture), "Captured texture");
                }
                sampled = Clock::now();
            }
            if(proof_gpu) {
                const RECT visible{0,0,static_cast<LONG>(width),static_cast<LONG>(height)};
                proof_result=proof_gpu->inspect_texture(texture.Get(),0,visible,"capture",0);
            }
            encode_started = Clock::now();
            encoder.encode(texture.Get(), index, packet);
            encode_us = static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(Clock::now()-encode_started).count());
            // Keep backend switching outside the outstanding DXGI frame lease.
            // Here only reject a layout transition before publishing the frame.
            // The selected output/DPI still has a per-frame guard. Full adapter
            // inventory remains in coordinate(), outside the frame hot path.
            if (!synthetic) validate_selected_display_layout(display);
        }
        if (!synthetic) texture.Reset();
        if (packet.empty() || packet.size() > max_packet) throw std::runtime_error("Encoded frame outside bounds");
        if(proof_gpu) {
            proof_output << "{\"frame\":" << index+1 << ",\"pattern\":" << proof_result.pattern
                << ",\"max_error\":" << proof_result.max_error << ",\"checks\":" << pattern_proof::samples
                << ",\"encoder\":\"" << encoder.name() << "\""
                << ",\"fingerprint\":\"" << std::hex << pattern_proof::fingerprint(packet) << std::dec << "\"}\n";
            proof_output.flush();
            if(!proof_output) throw std::runtime_error("Synthetic proof log write failed");
        }
        const auto native_total_us = static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(Clock::now()-capture_started).count());
        write_value(static_cast<uint32_t>(packet.size())); write_value(index);
        write_value(static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(sampled-start).count()));
        write_value(static_cast<uint32_t>(idr(packet)));
        if (command == 3 || command == 4) {
            write_value(static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(encode_started-capture_started).count()));
            write_value(encode_us);
        }
        if (command == 4) {
            write_value(native_total_us); write_value(encoder.readback_us());
            write_value(encoder.conversion_us()); write_value(encoder.codec_us());
        }
        write_bytes(packet.data(), static_cast<uint32_t>(packet.size()));
        ++index;
        // Anchor to the frame actually sampled, rather than a deadline from
        // before a credit/static-screen pause. Otherwise that pause allows
        // two immediate captures while the old schedule catches up.
        next = sampled + std::chrono::microseconds(1000000/fps);
    }
}
static void render_stream(VideoDecoder& decoder, NativePresenter& presenter, unsigned fps, bool interactive) {
    write_value(uint32_t(2)); // Version of the private event protocol, not network framing.
    write_value(uint8_t(4)); write_value(uint8_t(decoder.software() ? 2 : 1));
    uint64_t expected = 0;
    uint64_t cursor_sequence = 0;
    std::deque<std::pair<LONGLONG, uint64_t>> pending;
    std::deque<Clock::time_point> decode_started;
    auto last_output = Clock::now();
    const auto consume = [&](DecodedFrame& frame) {
        if (pending.empty() || pending.front().first != frame.timestamp)
            throw std::runtime_error("Decoder reordered or invented a timestamp");
        const auto index = pending.front().second;
        const auto rendering = Clock::now();
        const auto decode_us = static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(rendering-decode_started.front()).count());
        presenter.render(frame, index);
        const auto render_us = static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(Clock::now()-rendering).count());
        decode_started.pop_front();
        pending.pop_front(); last_output = Clock::now();
        if(presenter.pattern_proof_enabled()) {
            write_value(uint8_t(8)); write_value(index); write_value(uint32_t(presenter.proof_pattern));
            write_value(uint32_t(presenter.proof_decoded_error)); write_value(uint32_t(presenter.proof_rendered_error));
            write_value(uint32_t(pattern_proof::samples*2));
        }
        write_value(uint8_t(7)); write_value(index); write_value(decode_us); write_value(render_us);
    };
    const auto drain_ready = [&] {
        if (pending.empty()) return;
        decoder.receive(consume);
        if (!pending.empty() && Clock::now() - last_output > std::chrono::seconds(5))
            throw std::runtime_error("Decoder output exceeded bounded latency");
    };
    while (true) {
        const auto size = read_value<uint32_t>(&presenter, drain_ready);
        if (size == UINT32_MAX-1) {
            const auto length = read_value<uint32_t>(&presenter, drain_ready);
            if (length < 9 || length > max_cursor_bytes+64) throw std::runtime_error("Invalid cursor render packet");
            const auto sequence = read_value<uint64_t>(&presenter, drain_ready);
            if (cursor_sequence == UINT64_MAX || sequence != cursor_sequence+1) throw std::runtime_error("Invalid cursor render sequence");
            std::vector<uint8_t> packet(length-8); read_bytes(packet.data(),static_cast<uint32_t>(packet.size()),&presenter,drain_ready);
            presenter.cursor_update(packet); cursor_sequence = sequence;
            write_value(uint8_t(6)); write_value(sequence); continue;
        }
        if (interactive && size == UINT32_MAX) {
            const auto enabled = read_value<uint8_t>(&presenter, drain_ready);
            if (enabled > 1) throw std::runtime_error("Invalid viewer input mode");
            presenter.input_enabled(enabled != 0); continue;
        }
        if (!size) {
            decoder.drain(consume);
            if (!pending.empty()) throw std::runtime_error("Decoder did not drain all submitted frames");
            presenter.finish();
            write_value(uint8_t(3));
            write_value(presenter.displayed); write_value(presenter.pixel_checks); write_value(presenter.max_color_error);
            return;
        }
        if (size > max_packet) throw std::runtime_error("Render packet too large");
        const auto index = read_value<uint64_t>(&presenter, drain_ready);
        if (index != expected) throw std::runtime_error("Render frame sequence mismatch");
        std::vector<uint8_t> packet(size); read_bytes(packet.data(), size, &presenter, drain_ready);
        const LONGLONG timestamp = static_cast<LONGLONG>(index) * 10000000 / fps;
        const auto deadline = Clock::now() + std::chrono::seconds(2);
        const auto decoding = Clock::now();
        while (!decoder.try_submit(packet, timestamp)) {
            drain_ready(); presenter.pump();
            if (Clock::now() >= deadline) throw std::runtime_error("Decoder backpressure timed out");
            media_wait::sleep(std::chrono::milliseconds(1));
        }
        if (pending.empty()) last_output = Clock::now();
        pending.emplace_back(timestamp, index);
        decode_started.push_back(decoding);
        // Acceptance permits another compressed packet, but gives no network
        // credit and never enables input. Only event 2 confirms rendered output.
        write_value(uint8_t(1)); write_value(index); ++expected;
        drain_ready();
        if (pending.size() > 4) throw std::runtime_error("Decoder exceeds the four-frame latency budget");
    }
}
static void render(const Display& display, unsigned width, unsigned height, unsigned fps, bool verify, HWND parent, unsigned rotation, bool interactive, bool streaming, bool force_software, bool proof = false) {
    MediaRuntime runtime;
    ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context;
    check(D3D11CreateDevice(display.adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, &context), "Render device");
    VideoDecoder decoder(device.Get(), width, height, fps, force_software);
    NativePresenter presenter;
    presenter.input_available(interactive);
    presenter.initialize(device.Get(), width, height, fps, display.screen.DesktopCoordinates, true, verify, parent, rotation);
    if(proof) presenter.enable_pattern_proof();
    write_value(magic);
    if (streaming) {
        try { render_stream(decoder, presenter, fps, interactive); }
        catch (const std::exception& error) {
            // Preserve the same cause on the presentation pipe: either reader
            // can observe process termination first.
            const std::string message = std::string(error.what()).substr(0, 512);
            try {
                write_value(uint8_t(5)); write_value(uint16_t(message.size()));
                write_bytes(message.data(), static_cast<uint32_t>(message.size()));
            } catch (...) {} // A closed parent pipe must not replace the cause.
            throw;
        }
        return;
    }
    uint64_t expected = 0;
    while (true) {
        const auto size = read_value<uint32_t>(&presenter);
        if (interactive && size == UINT32_MAX) {
            const auto enabled = read_value<uint8_t>(&presenter);
            if (enabled > 1) throw std::runtime_error("Invalid viewer input mode");
            presenter.input_enabled(enabled != 0); continue;
        }
        if (!size) {
            presenter.finish();
            write_value(presenter.displayed); write_value(presenter.pixel_checks); write_value(presenter.max_color_error);
            return;
        }
        if (size > max_packet) throw std::runtime_error("Render packet too large");
        const auto index = read_value<uint64_t>(&presenter);
        if (index != expected) throw std::runtime_error("Render frame sequence mismatch");
        std::vector<uint8_t> packet(size); read_bytes(packet.data(), size, &presenter);
        const LONGLONG timestamp = static_cast<LONGLONG>(index) * 10000000 / fps;
        decoder.submit(packet, timestamp);
        unsigned received = 0;
        decoder.receive([&](DecodedFrame& frame) {
            if (frame.timestamp != timestamp || received != 0)
                throw std::runtime_error("Decoder reordered or duplicated a frame");
            presenter.render(frame, index);
            ++received;
        });
        if (received != 1)
            throw std::runtime_error("Decoder buffered a frame; asynchronous output adapter required");
        write_value(index); ++expected;
    }
}
extern "C" int pab_media_main(int argc, const char* const* argv) {
    const bool interactive_errors = argc >= 6 && std::string(argv[1]) == "render" &&
        (std::string(argv[5]) == "interactive" || std::string(argv[5]) == "stream-interactive" ||
         std::string(argv[5]) == "interactive-software" || std::string(argv[5]) == "stream-interactive-software");
    try {
        if (const auto result = user_capture::run(argc, argv)) return *result;
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if (argc == 2 && std::string(argv[1]) == "media-timing-check") {
            media_timing_check(); return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "pattern-proof-check") {
            pattern_proof::fixture(); std::cout << "{\"status\":\"passed\",\"negative_cases\":15,\"desktop_access\":false}\n"; return 0;
        }
        if (argc == 3 && std::string(argv[1]) == "media-priority-check" && std::string(argv[2]) == "inventory") {
            if (GetPriorityClass(GetCurrentProcess()) != NORMAL_PRIORITY_CLASS)
                throw std::runtime_error("Media worker inherited background priority");
            std::cout << R"([{"id":"media-priority-fixture","name":"Synthetic priority fixture","x":0,"y":0,"desktop_width":64,"desktop_height":64,"pixel_width":64,"pixel_height":64,"scale_milli":1000,"rotation":"identity","primary":true}])" << '\n'; return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "viewer-chrome-check") {
            viewer_chrome_check(); return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "cursor-win32-check") {
            win32_cursor_check(); return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "cursor-shape-check") {
            cursor_shape_check(); return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "local-cursor-check") {
            local_cursor_check(); return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "cursor-overlay-check") {
            cursor_overlay_check(); return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "input-recovery-check") {
            input_recovery_check(); return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "recover-input") {
            InputReleases releases(input_recovery_path()); recover_input(releases); return 0;
        }
        const auto found = displays();
        if (argc == 2 && std::string(argv[1]) == "display-layout-check") {
            if (found.empty()) throw std::runtime_error("No display for layout check");
            unsigned rejected = 0;
            for (const auto& current : found) {
                validate_selected_display_layout(current);
                for (unsigned mutation = 0; mutation < 5; ++mutation) {
                    auto stale = current;
                    switch (mutation) {
                        case 0: ++stale.screen.DesktopCoordinates.right; break;
                        case 1: stale.screen.Rotation = current.screen.Rotation == DXGI_MODE_ROTATION_IDENTITY
                            ? DXGI_MODE_ROTATION_ROTATE90 : DXGI_MODE_ROTATION_IDENTITY; break;
                        case 2: stale.screen.Monitor = nullptr; break;
                        case 3: ++stale.dpi_x; break;
                        case 4: ++stale.dpi_y; break;
                    }
                    try { validate_selected_display_layout(stale); }
                    catch (const std::runtime_error&) { ++rejected; continue; }
                    throw std::runtime_error("Stale capture layout accepted");
                }
            }
            std::cout << "{\"displays\":" << found.size() << ",\"stale_layouts_rejected\":" << rejected << "}\n";
            return 0;
        }
        if (argc == 3 && std::string(argv[1]) == "input-fixture" && !found.empty()) {
            input_fixture(found.front(), number(argv[2], 120)); return 0;
        }
        if (argc == 4 && std::string(argv[1]) == "input-fixture") {
            for (const auto& d : found) if (screen_id(d) == argv[2]) {
                input_fixture(d, number(argv[3], 120)); return 0;
            }
            throw std::runtime_error("Selected fixture screen disappeared");
        }
        if (argc == 3 && std::string(argv[1]) == "input") {
            for (const auto& d : found) if (screen_id(d) == argv[2]) { input_worker(d); return 0; }
            throw std::runtime_error("Selected input screen disappeared");
        }
        if ((argc == 3 || argc == 4) && std::string(argv[1]) == "cursor") {
            for (const auto& d : found) if (screen_id(d) == argv[2]) {
                cursor_worker(d, argc == 4 ? argv[3] : ""); return 0;
            }
            throw std::runtime_error("Selected cursor screen disappeared");
        }
        if (argc == 2 && std::string(argv[1]) == "inventory") { inventory(found); return 0; }
        if (argc == 4 && std::string(argv[1]) == "animate-display" && !found.empty()) {
            const auto fps = number(argv[2], 60), seconds = number(argv[3], 120);
            ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context;
            check(D3D11CreateDevice(found.front().adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0, D3D11_SDK_VERSION,
                &device, nullptr, &context), "Animation device");
            PatternWindow window; window.initialize(device.Get(), found.front().screen.DesktopCoordinates);
            const auto start = Clock::now();
            for (uint64_t frame = 0; frame < uint64_t(fps)*seconds; ++frame) {
                window.draw(context.Get(), frame);
                media_wait::until(start + std::chrono::microseconds((frame+1)*1000000/fps));
            }
            return 0;
        }
        if ((argc == 5 || argc == 6 || argc == 7) && std::string(argv[1]) == "capture") {
            std::string mode(argv[4]);
            const bool fail_encoder = mode.size() > 13 && mode.compare(mode.size()-13,13,"-fail-encoder") == 0;
            if (fail_encoder) mode.resize(mode.size()-13);
            const bool software = mode.size() > 9 && mode.compare(mode.size()-9,9,"-software") == 0;
            if (software) mode.resize(mode.size()-9);
            for (const auto& d : found) if (screen_id(d) == argv[2]) {
                capture(d, number(argv[3],60),mode,software,fail_encoder,argc >= 6 ? argv[5] : "",argc == 7 ? argv[6] : ""); return 0;
            }
            throw std::runtime_error("Selected screen disappeared");
        }
        if (argc >= 6 && argc <= 8 && std::string(argv[1]) == "render" && !found.empty()) {
            std::string mode(argv[5]);
            const bool software = mode.size() > 9 && mode.compare(mode.size()-9,9,"-software") == 0;
            if (software) mode.resize(mode.size()-9);
            const bool streaming = mode == "stream" || mode == "stream-verify" || mode == "stream-interactive" || mode == "stream-proof";
            if (!streaming && mode != "verify" && mode != "normal" && mode != "interactive") throw std::runtime_error("Invalid render mode");
            const auto width = number(argv[2],8192), height = number(argv[3],8192);
            if (uint64_t(width)*height > 33554432) throw std::runtime_error("Render dimensions too large");
            HWND parent = nullptr;
            if (argc >= 7) {
                size_t end = 0; const std::string handle(argv[6]);
                parent = reinterpret_cast<HWND>(std::stoull(handle, &end));
                if (end != handle.size() || (parent && !IsWindow(parent))) throw std::runtime_error("Invalid viewer owner");
            }
            unsigned rotation = 0;
            if (argc == 8) {
                const std::string angle(argv[7]);
                if (angle == "90") rotation = 1;
                else if (angle == "180") rotation = 2;
                else if (angle == "270") rotation = 3;
                else if (angle != "0") throw std::runtime_error("Invalid rotation");
            }
            render(found.front(),width,height,number(argv[4],60),mode == "verify" || mode == "stream-verify", parent, rotation,
                mode == "interactive" || mode == "stream-interactive", streaming, software,mode == "stream-proof"); return 0;
        }
        throw std::runtime_error("usage: stream-media inventory | capture SCREEN_ID FPS synthetic|desktop|animate | render WIDTH HEIGHT FPS normal|verify");
    } catch (const std::exception& e) {
        if (interactive_errors) {
            // stderr is the input event pipe in interactive mode. Plain text
            // here used to become an invalid input packet, hiding the actual
            // renderer failure behind a generic pipe-closed error.
            const std::string message = std::string(e.what()).substr(0, 512);
            const uint16_t length = static_cast<uint16_t>(message.size());
            const uint8_t header[8]{7, uint8_t(length), uint8_t(length >> 8), 0, 0, 0, 0, 0};
            DWORD written = 0;
            if (WriteFile(GetStdHandle(STD_ERROR_HANDLE), header, sizeof(header), &written, nullptr) && written == sizeof(header))
                WriteFile(GetStdHandle(STD_ERROR_HANDLE), message.data(), length, &written, nullptr);
        } else {
            std::cerr << e.what() << '\n';
        }
        return 1;
    }
}
